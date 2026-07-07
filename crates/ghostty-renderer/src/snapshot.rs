//! The `RenderSnapshot` contract: what any renderer backend needs to draw
//! one frame, decoupled from `ghostty-vt`'s internal page/pin
//! representation.
//!
//! This module also carries the one piece of `src/renderer/State.zig`
//! (commit `2da015cd6`) that's timeless geometry rather than a threading
//! concern: [`Preedit`] (IME composition text placement). See
//! `docs/analysis/renderer-r0.md` for the full design writeup — why `State`
//! itself isn't ported as a struct, why there are two planned
//! implementations, and the precise list of what the future `DirtySnapshot`
//! impl needs from `ghostty-vt`.

use ghostty_vt::color::{Palette, Rgb};
use ghostty_vt::page::size::CellCountInt;
use ghostty_vt::snapshot::{SnapshotCell, SnapshotCursor};
use ghostty_vt::terminal::Terminal;

/// What changed since the last frame a renderer consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyStatus {
    /// Everything must be repainted (first frame, resize, palette swap,
    /// clear, etc).
    Full,
    /// Only the rows in `dirty_rows` changed; cursor/palette/preedit may
    /// also have changed independently of any row — check those directly
    /// rather than assuming they track row dirtiness.
    Partial { dirty_rows: Vec<usize> },
}

/// A single codepoint in an IME preedit (composition) string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreeditCodepoint {
    pub codepoint: u32,
    pub wide: bool,
}

/// The pre-edit (IME composition) state. Port of `State.Preedit`
/// (`src/renderer/State.zig`). No `ghostty-vt` producer wires this yet (see
/// `docs/analysis/renderer-r0.md`); this type exists so the trait and a
/// future input-layer chunk have a shape to fill in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preedit {
    pub codepoints: Vec<PreeditCodepoint>,
}

/// The result of [`Preedit::range`]: where preedit text should actually be
/// drawn, and which leading codepoints to skip if it doesn't fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreeditRange {
    pub start: CellCountInt,
    pub end: CellCountInt,
    pub cp_offset: usize,
}

impl Preedit {
    /// The width in cells of all codepoints in the preedit.
    pub fn width(&self) -> usize {
        self.codepoints
            .iter()
            .map(|cp| if cp.wide { 2 } else { 1 })
            .sum()
    }

    /// Range returns the start and end x position of the preedit text along
    /// with any codepoint offset necessary to fit the preedit into the
    /// available space.
    pub fn range(&self, start: CellCountInt, max: CellCountInt) -> PreeditRange {
        // If our width is greater than the number of cells we have then we
        // need to adjust our codepoint start to a point where our width
        // would be less than the number of cells we have.
        //
        // max is inclusive, so we need to add 1 to it.
        let max_width = max - start + 1;

        // Rebuild our width in reverse order. This is because we want to
        // offset by the end cells, not the start cells (if we have to).
        let mut w: CellCountInt = 0;
        let mut cp_offset = 0;
        let mut found = false;
        for i in 0..self.codepoints.len() {
            let reverse_i = self.codepoints.len() - i - 1;
            let cp = &self.codepoints[reverse_i];
            w += if cp.wide { 2 } else { 1 };
            if w > max_width {
                cp_offset = reverse_i;
                found = true;
                break;
            }
        }
        if !found {
            cp_offset = 0;
        }

        // If our preedit goes off the end of the screen, we adjust it so
        // that it shifts left.
        let end = if w > 0 { start + (w - 1) } else { start };
        let start_offset = end.saturating_sub(max);
        PreeditRange {
            start: start.saturating_sub(start_offset),
            end: end.saturating_sub(start_offset),
            cp_offset,
        }
    }
}

/// A single kitty graphics placement visible this frame. Placeholder: the
/// `kitty::Placement`/`ImageStorage` model already exists in `ghostty-vt`
/// but isn't threaded through `Snapshot`/`SnapshotWindow` yet (see
/// `docs/analysis/renderer-r0.md`). No implementation constructs this today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyPlacement {
    /// Grid column/row of the placement's top-left cell.
    pub col: usize,
    pub row: usize,
    pub columns: u32,
    pub rows: u32,
}

/// Everything a renderer needs to draw one frame, decoupled from
/// `ghostty-vt`'s internal page/pin representation.
///
/// Two planned implementations:
/// - [`FullSnapshot`] (implemented now): wraps
///   [`ghostty_vt::snapshot::SnapshotWindow`], rebuilt fresh (a full
///   visible-window copy) every frame. Simple and correct, and already
///   cheap (O(visible rows), not O(scrollback)) thanks to
///   `Screen::snapshot_window`'s backward-walk-from-bottom design — but
///   still re-copies every cell every frame even when only one row changed.
/// - `DirtySnapshot` (contract only — lands with a future chunk once
///   `PageList`'s existing per-row dirty bit is threaded through to the
///   snapshot boundary; see `docs/analysis/renderer-r0.md` for the precise
///   list of what that chunk needs from `ghostty-vt`).
pub trait RenderSnapshot {
    /// Number of columns / visible rows this snapshot covers. Always
    /// matches the renderer's current grid size for this frame.
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;

    /// What changed since the renderer's last consumed frame.
    fn dirty(&self) -> DirtyStatus;

    /// One visible row, 0-indexed from the top of the rendered window.
    /// Always exactly `cols()` cells long.
    fn row(&self, row: usize) -> &[SnapshotCell];

    /// Cursor to draw this frame, or `None` if it's outside the visible
    /// viewport. This is position/raw-visibility data only; the actual
    /// drawn *style* (block/bar/hollow/etc, or suppressed entirely by
    /// focus/blink/preedit) is decided by [`crate::cursor::style`] using
    /// renderer-local options this trait doesn't carry.
    fn cursor(&self) -> Option<SnapshotCursor>;

    /// Current 256-color palette.
    fn palette(&self) -> &Palette;
    /// The dynamic default foreground (OSC 10/110), if set.
    fn default_fg(&self) -> Option<Rgb>;
    /// The dynamic default background (OSC 11/111), if set.
    fn default_bg(&self) -> Option<Rgb>;

    /// IME composition text to render over/near the cursor, if active.
    /// Placeholder: no `ghostty-vt` producer wires this yet (see module
    /// docs); implementations return `None` until an input-layer chunk
    /// exists.
    fn preedit(&self) -> Option<&Preedit>;

    /// Kitty graphics placements visible this frame. Placeholder: returns
    /// an empty slice until placement windowing is wired (see module
    /// docs).
    fn kitty_placements(&self) -> &[KittyPlacement];
}

/// A full-copy [`RenderSnapshot`] implementation backed by
/// `ghostty_vt::Terminal::snapshot_window`. Always reports
/// [`DirtyStatus::Full`] and re-copies the entire visible window every
/// frame; correct but not incremental. See the trait docs and
/// `docs/analysis/renderer-r0.md` for the planned `DirtySnapshot`
/// alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullSnapshot {
    window: ghostty_vt::snapshot::SnapshotWindow,
}

impl FullSnapshot {
    /// Capture a full-copy snapshot of `terminal`'s currently visible
    /// window, `scrollback_offset` rows up from the bottom (0 = the live
    /// active area). Thin wrapper over `Terminal::snapshot_window`.
    pub fn capture(terminal: &Terminal, scrollback_offset: usize) -> FullSnapshot {
        FullSnapshot {
            window: terminal.snapshot_window(scrollback_offset),
        }
    }
}

impl RenderSnapshot for FullSnapshot {
    fn cols(&self) -> usize {
        self.window.cols
    }

    fn rows(&self) -> usize {
        self.window.window.len()
    }

    fn dirty(&self) -> DirtyStatus {
        // A fresh full copy has no notion of "since last frame" — always
        // report Full. See DirtySnapshot (future chunk) for partial
        // reporting.
        DirtyStatus::Full
    }

    fn row(&self, row: usize) -> &[SnapshotCell] {
        &self.window.window[row].cells
    }

    fn cursor(&self) -> Option<SnapshotCursor> {
        // The active area (what snapshot_window captures) always contains
        // the live cursor today; "cursor scrolled out of the visible
        // window" isn't modeled by ghostty-vt yet. See
        // docs/analysis/renderer-r0.md's cursor.zig section.
        Some(self.window.cursor)
    }

    fn palette(&self) -> &Palette {
        &self.window.palette
    }

    fn default_fg(&self) -> Option<Rgb> {
        self.window.default_fg
    }

    fn default_bg(&self) -> Option<Rgb> {
        self.window.default_bg
    }

    fn preedit(&self) -> Option<&Preedit> {
        // No producer wired yet; see module docs.
        None
    }

    fn kitty_placements(&self) -> &[KittyPlacement] {
        // No producer wired yet; see module docs.
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_vt::stream::{Stream, TerminalHandler};
    use ghostty_vt::terminal::Options;

    fn feed(cols: u16, rows: u16, bytes: &[u8]) -> Terminal {
        let term = Terminal::new(Options {
            cols,
            rows,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(bytes);
        stream.handler.terminal
    }

    fn row_text(cells: &[SnapshotCell]) -> String {
        let mut s: String = cells
            .iter()
            .filter(|c| !c.is_spacer())
            .map(|c| c.ch)
            .collect();
        while s.ends_with(' ') {
            s.pop();
        }
        s
    }

    #[test]
    fn preedit_range_covers_exact_cell_width() {
        let p = Preedit {
            codepoints: vec![PreeditCodepoint {
                codepoint: 'a' as u32,
                wide: false,
            }],
        };
        let range = p.range(2, 9);
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 2);
        assert_eq!(range.cp_offset, 0);

        // U+AC00 HANGUL SYLLABLE GA, wide.
        let p = Preedit {
            codepoints: vec![PreeditCodepoint {
                codepoint: 0xAC00,
                wide: true,
            }],
        };
        let range = p.range(2, 9);
        assert_eq!(range.start, 2);
        assert_eq!(range.end, 3);
        assert_eq!(range.cp_offset, 0);
    }

    #[test]
    fn preedit_range_shifts_left_at_right_edge() {
        let p = Preedit {
            codepoints: vec![PreeditCodepoint {
                codepoint: 0xAC00,
                wide: true,
            }],
        };
        let range = p.range(9, 9);
        assert_eq!(range.start, 8);
        assert_eq!(range.end, 9);
        assert_eq!(range.cp_offset, 0);
    }

    #[test]
    fn full_snapshot_reports_full_dirty_and_matches_terminal() {
        let term = feed(10, 3, b"Hello");
        let snap = FullSnapshot::capture(&term, 0);

        assert_eq!(snap.dirty(), DirtyStatus::Full);
        assert_eq!(snap.cols(), 10);
        assert_eq!(snap.rows(), 3);
        assert_eq!(row_text(snap.row(0)), "Hello");
        assert!(snap.cursor().is_some());
        assert!(snap.preedit().is_none());
        assert!(snap.kitty_placements().is_empty());
    }

    #[test]
    fn full_snapshot_stays_coherent_across_writes() {
        let mut term = feed(10, 3, b"one");

        let snap1 = FullSnapshot::capture(&term, 0);
        assert_eq!(row_text(snap1.row(0)), "one");

        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(b"\r\ntwo");
        term = stream.handler.terminal;

        let snap2 = FullSnapshot::capture(&term, 0);
        assert_eq!(row_text(snap2.row(0)), "one");
        assert_eq!(row_text(snap2.row(1)), "two");
    }

    #[test]
    fn full_snapshot_reflects_resize() {
        let term = feed(20, 5, b"hello world");
        let snap = FullSnapshot::capture(&term, 0);
        assert_eq!(snap.cols(), 20);
        assert_eq!(snap.rows(), 5);
        assert_eq!(row_text(snap.row(0)), "hello world");
    }

    #[test]
    fn full_snapshot_palette_and_colors_track_terminal() {
        let term = feed(10, 2, b"\x1b]4;1;#112233\x1b\\\x1b]11;#001122\x1b\\");
        let snap = FullSnapshot::capture(&term, 0);
        assert_eq!(snap.palette()[1], Rgb::new(0x11, 0x22, 0x33));
        assert_eq!(snap.default_bg(), Some(Rgb::new(0x00, 0x11, 0x22)));
        assert_eq!(snap.default_fg(), None);
    }

    #[test]
    fn full_snapshot_over_scrollback_offset_matches_window() {
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let full = term.snapshot();
        assert!(full.scrollback_len() >= 2);

        for offset in 0..=full.scrollback_len() {
            let snap = FullSnapshot::capture(&term, offset);
            let expected = full.visible_window(offset);
            for (row_idx, expected_row) in expected.iter().enumerate() {
                assert_eq!(snap.row(row_idx), expected_row.cells.as_slice());
            }
        }
    }
}
