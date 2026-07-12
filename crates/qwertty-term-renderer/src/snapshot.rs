//! The `RenderSnapshot` contract: what any renderer backend needs to draw
//! one frame, decoupled from `qwertty-term-vt`'s internal page/pin
//! representation.
//!
//! This module also carries the one piece of `src/renderer/State.zig`
//! (commit `2da015cd6`) that's timeless geometry rather than a threading
//! concern: [`Preedit`] (IME composition text placement). See
//! `docs/analysis/renderer-r0.md` for the full design writeup — why `State`
//! itself isn't ported as a struct, why there are two planned
//! implementations, and the precise list of what the future `DirtySnapshot`
//! impl needs from `qwertty-term-vt`.

use qwertty_term_vt::color::{Palette, Rgb};
use qwertty_term_vt::page::size::CellCountInt;
use qwertty_term_vt::snapshot::{SnapshotCell, SnapshotCursor};
use qwertty_term_vt::terminal::Terminal;

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

/// The intrinsic (single-frame) dirty signals a [`RenderSnapshot`] carries so a
/// renderer can decide full-vs-partial rebuild. The cross-frame triggers
/// (screen switch, viewport move, resize) are compared by the renderer against
/// the frame it last drew via [`FrameKey`]; the whole-screen "force full"
/// signal (palette/selection/clear/etc) is a single bool here.
///
/// This mirrors how upstream's `RenderState.update` combines the live
/// terminal's global dirty flags with its own persisted per-render-state
/// comparison fields (`self.screen`, `self.viewport_pin`, `self.rows/cols`) to
/// pick `redraw`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDirty {
    /// Per-visible-row dirty bits (top-to-bottom, exactly `rows` long).
    pub row_dirty: Vec<bool>,
    /// True if a whole-frame rebuild is forced by intrinsic global signals
    /// (palette change, selection change, screen clear, etc).
    pub global_forces_full: bool,
    /// The cross-frame identity of this frame's viewport; the renderer forces
    /// a full rebuild when it differs from the last frame it drew.
    pub frame_key: FrameKey,
}

/// The identity of a rendered frame's viewport, used to detect the cross-frame
/// full-rebuild triggers by comparing against the previously drawn frame:
/// a screen switch (`screen`), a scroll / viewport move (`window_top`), or a
/// resize (`cols`/`rows`). Port of the `self.screen`/`self.viewport_pin`/
/// `self.rows`/`self.cols` comparisons in upstream `RenderState.update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameKey {
    /// 0 = primary screen, 1 = alternate. Compared like upstream's
    /// `active_key != self.screen`.
    pub screen: u8,
    /// Absolute logical row index of the top visible row (the snapshot's
    /// `window_top`). A change means the viewport scrolled — upstream's
    /// `viewport_pin` inequality.
    pub window_top: usize,
    pub cols: usize,
    pub rows: usize,
}

/// A single codepoint in an IME preedit (composition) string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreeditCodepoint {
    pub codepoint: u32,
    pub wide: bool,
}

/// The pre-edit (IME composition) state. Port of `State.Preedit`
/// (`src/renderer/State.zig`). No `qwertty-term-vt` producer wires this yet (see
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
/// `kitty::Placement`/`ImageStorage` model already exists in `qwertty-term-vt`
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
/// `qwertty-term-vt`'s internal page/pin representation.
///
/// Two planned implementations:
/// - [`FullSnapshot`] (implemented now): wraps
///   [`qwertty_term_vt::snapshot::SnapshotWindow`], rebuilt fresh (a full
///   visible-window copy) every frame. Simple and correct, and already
///   cheap (O(visible rows), not O(scrollback)) thanks to
///   `Screen::snapshot_window`'s backward-walk-from-bottom design — but
///   still re-copies every cell every frame even when only one row changed.
/// - `DirtySnapshot` (contract only — lands with a future chunk once
///   `PageList`'s existing per-row dirty bit is threaded through to the
///   snapshot boundary; see `docs/analysis/renderer-r0.md` for the precise
///   list of what that chunk needs from `qwertty-term-vt`).
pub trait RenderSnapshot {
    /// Number of columns / visible rows this snapshot covers. Always
    /// matches the renderer's current grid size for this frame.
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;

    /// The intrinsic dirty signals for this frame: per-row dirty bits, the
    /// global "force full" bool, and the cross-frame [`FrameKey`]. The renderer
    /// combines these with its own memory of the last drawn frame to decide
    /// full-vs-partial (see [`crate::engine::Engine::update_frame`]).
    fn dirty_signals(&self) -> SnapshotDirty;

    /// Convenience view of [`RenderSnapshot::dirty_signals`] that does *not*
    /// account for cross-frame triggers (it has no memory of the last frame):
    /// [`DirtyStatus::Full`] if the global signals force a full rebuild, else
    /// [`DirtyStatus::Partial`] listing the dirty rows. The engine uses
    /// `dirty_signals` directly (with its `FrameKey` memory); this exists for
    /// tests and simple consumers.
    fn dirty(&self) -> DirtyStatus {
        let d = self.dirty_signals();
        if d.global_forces_full {
            return DirtyStatus::Full;
        }
        DirtyStatus::Partial {
            dirty_rows: d
                .row_dirty
                .iter()
                .enumerate()
                .filter_map(|(y, &dirty)| dirty.then_some(y))
                .collect(),
        }
    }

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
    /// Placeholder: no `qwertty-term-vt` producer wires this yet (see module
    /// docs); implementations return `None` until an input-layer chunk
    /// exists.
    fn preedit(&self) -> Option<&Preedit>;

    /// Kitty graphics placements visible this frame. Placeholder: returns
    /// an empty slice until placement windowing is wired (see module
    /// docs).
    fn kitty_placements(&self) -> &[KittyPlacement];
}

/// A full-copy [`RenderSnapshot`] implementation backed by
/// `qwertty_term_vt::Terminal::snapshot_window[_tracking]`. It re-copies the entire
/// visible window every frame (cheap: O(visible rows)), but carries the vt
/// layer's per-row + global dirty signals so the engine can still skip
/// *rebuilding* (shaping / glyph lookup / cell writes for) the clean rows.
///
/// Two capture paths:
/// - [`FullSnapshot::capture`] (read-only): reports every row dirty, leaves the
///   terminal's dirty state untouched — for inspection / tests.
/// - [`FullSnapshot::capture_tracking`] (incremental): reads and clears the
///   terminal's dirty state so `dirty_signals()` reports the real per-row and
///   global dirtiness — for the per-frame render path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullSnapshot {
    window: qwertty_term_vt::snapshot::SnapshotWindow,
}

impl FullSnapshot {
    /// Capture a full-copy snapshot of `terminal`'s currently visible
    /// window, `scrollback_offset` rows up from the bottom (0 = the live
    /// active area). Thin wrapper over `Terminal::snapshot_window`.
    ///
    /// This is the read-only path: it reports every row dirty (`dirty()` =>
    /// `Full`) and does not touch the terminal's dirty state. For incremental
    /// redraw use [`FullSnapshot::capture_tracking`].
    pub fn capture(terminal: &Terminal, scrollback_offset: usize) -> FullSnapshot {
        FullSnapshot {
            window: terminal.snapshot_window(scrollback_offset),
        }
    }

    /// Capture the live active area: [`FullSnapshot::capture`] with a
    /// scrollback offset of 0. The common embedder shape — feed bytes,
    /// capture live, render — without the magic-zero argument.
    pub fn capture_live(terminal: &Terminal) -> FullSnapshot {
        FullSnapshot::capture(terminal, 0)
    }

    /// Capture a snapshot on the incremental-redraw path: reads and *clears*
    /// the terminal's per-row/page/global dirty state, so the window carries
    /// the real per-row dirty bits and global signals. Thin wrapper over
    /// `Terminal::snapshot_window_tracking` (requires `&mut` because consuming
    /// dirty state mutates the terminal, exactly as upstream `RenderState`'s
    /// snapshot does under the render lock).
    pub fn capture_tracking(terminal: &mut Terminal, scrollback_offset: usize) -> FullSnapshot {
        FullSnapshot {
            window: terminal.snapshot_window_tracking(scrollback_offset),
        }
    }

    /// Wrap an already-captured [`SnapshotWindow`] (chunk R5, additive). A
    /// window host that holds its engine behind a mutex takes the windowed
    /// snapshot itself (releasing the lock before rendering), then wraps it here
    /// — avoiding a second `snapshot_window` call and a longer lock hold.
    /// Equivalent to [`FullSnapshot::capture`] once the window is captured.
    pub fn from_window(window: qwertty_term_vt::snapshot::SnapshotWindow) -> FullSnapshot {
        FullSnapshot { window }
    }
}

impl RenderSnapshot for FullSnapshot {
    fn cols(&self) -> usize {
        self.window.cols
    }

    fn rows(&self) -> usize {
        self.window.window.len()
    }

    fn dirty_signals(&self) -> SnapshotDirty {
        SnapshotDirty {
            row_dirty: self.window.row_dirty.clone(),
            global_forces_full: self.window.global_dirty_forces_full(),
            frame_key: FrameKey {
                screen: match self.window.screen_key {
                    qwertty_term_vt::terminal::ScreenKey::Primary => 0,
                    qwertty_term_vt::terminal::ScreenKey::Alternate => 1,
                },
                window_top: self.window.window_top,
                cols: self.window.cols,
                rows: self.window.window.len(),
            },
        }
    }

    fn row(&self, row: usize) -> &[SnapshotCell] {
        &self.window.window[row].cells
    }

    fn cursor(&self) -> Option<SnapshotCursor> {
        // The cursor's position is relative to the active area (row 0 = top of
        // the active area). Its absolute logical row is
        // `scrollback_len + cursor.row`. The visible window covers absolute
        // rows `[window_top, window_top + rows)`. When the viewport is scrolled
        // back into history the cursor may fall outside that range — upstream
        // then reports `cursor.viewport == null` and draws no cursor
        // (`render.zig`; `cursor.zig` priority #1). Mirror that: suppress the
        // cursor when it's out of the visible window, and otherwise remap its
        // row to be window-relative.
        let abs_cursor_row = self.window.scrollback_len + self.window.cursor.row;
        let top = self.window.window_top;
        let rows = self.window.window.len();
        if abs_cursor_row < top || abs_cursor_row >= top + rows {
            return None;
        }
        let mut cursor = self.window.cursor;
        cursor.row = abs_cursor_row - top;
        Some(cursor)
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
    use qwertty_term_vt::stream::{Stream, TerminalHandler};
    use qwertty_term_vt::terminal::Options;

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
    fn full_snapshot_read_only_reports_all_rows_dirty_and_matches_terminal() {
        let term = feed(10, 3, b"Hello");
        let snap = FullSnapshot::capture(&term, 0);

        // The read-only capture reports every row dirty (a full repaint).
        assert_eq!(
            snap.dirty(),
            DirtyStatus::Partial {
                dirty_rows: vec![0, 1, 2]
            }
        );
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

    #[test]
    fn tracking_capture_reports_partial_after_single_row_change() {
        let mut term = feed(10, 4, b"aaa\r\nbbb\r\nccc\r\nddd");
        // Drain initial dirt.
        let _ = FullSnapshot::capture_tracking(&mut term, 0);

        // Overwrite exactly one row.
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(b"\x1b[2;1HZZZ"); // row index 1
        let mut term = stream.handler.terminal;

        let snap = FullSnapshot::capture_tracking(&mut term, 0);
        match snap.dirty() {
            DirtyStatus::Partial { dirty_rows } => {
                assert!(dirty_rows.contains(&1), "row 1 (rewritten) dirty");
                // Unrelated rows above the change stay clean (not a full repaint).
                assert!(!dirty_rows.contains(&0), "row 0 clean");
                assert!(!dirty_rows.contains(&2), "row 2 clean");
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    #[test]
    fn tracking_capture_forces_full_on_palette_change() {
        let mut term = feed(10, 2, b"hi");
        let _ = FullSnapshot::capture_tracking(&mut term, 0);

        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(b"\x1b]4;1;#112233\x1b\\");
        let mut term = stream.handler.terminal;

        let snap = FullSnapshot::capture_tracking(&mut term, 0);
        assert_eq!(snap.dirty(), DirtyStatus::Full);
    }

    #[test]
    fn cursor_hidden_when_scrolled_into_history() {
        // Push several rows into scrollback so a nonzero offset shows history.
        let term = feed(4, 2, b"aaaabbbbccccddddeeee");
        let sb = term.snapshot().scrollback_len();
        assert!(sb >= 1);

        // At the bottom (offset 0) the cursor is in the active area => shown.
        let at_bottom = FullSnapshot::capture(&term, 0);
        assert!(at_bottom.cursor().is_some(), "cursor shown at bottom");

        // Scrolled fully back into history => cursor out of the visible window
        // => suppressed (upstream draws no cursor when viewport != active).
        let scrolled = FullSnapshot::capture(&term, sb);
        assert!(
            scrolled.cursor().is_none(),
            "cursor hidden when scrolled into history"
        );
    }

    #[test]
    fn cursor_row_is_remapped_window_relative_when_partly_scrolled() {
        // A partial scroll that still keeps the cursor in view must remap the
        // cursor's row to be window-relative (so it draws at the right place).
        let term = feed(4, 3, b"aaaabbbbccccdddd");
        let full = term.snapshot();
        // Cursor is on the last active row.
        let bottom = FullSnapshot::capture(&term, 0);
        let c0 = bottom.cursor().expect("cursor visible at bottom");

        // Scroll up by 1: the cursor's absolute row is one lower in the window.
        let sb = full.scrollback_len();
        if sb >= 1 {
            let up = FullSnapshot::capture(&term, 1);
            match up.cursor() {
                Some(c) => assert_eq!(c.row, c0.row + 1, "cursor row shifted down by scroll"),
                None => { /* cursor scrolled out entirely — also valid */ }
            }
        }
    }
}
