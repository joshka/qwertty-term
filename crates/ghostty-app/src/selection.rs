//! App-side selection: viewport↔cell coordinate math and the CPU-side render
//! tint applied to a [`SnapshotWindow`] before it's handed to the renderer.
//!
//! `ghostty-vt`'s [`Screen`](ghostty_vt::screen::Screen) owns the actual
//! [`Selection`](ghostty_vt::screen::selection::Selection) value (a pair of
//! pins) and the mouse-drag-independent query/adjust primitives
//! (`docs/analysis/selection.md`). Neither the `RenderSnapshot` trait nor
//! `ghostty-renderer`'s cell engine (`docs/analysis/renderer-r5.md`'s
//! deferrals list) carries any selection state — `FrameOptions` has no
//! selection colors, and `Contents::rebuild_row` has no selection branch.
//! Rather than add that surface to two additive-only crates for a single
//! consumer, this module does the **least invasive correct thing**: after the
//! app takes its per-frame [`SnapshotWindow`] (`Engine::snapshot_window`), it
//! overlays the selection here, CPU-side, by swapping the selected cells' fg/
//! bg [`SnapshotColor`]s before wrapping the window in a `FullSnapshot`. This
//! needs no changes to `ghostty-vt` or `ghostty-renderer`.
//!
//! The window the app renders is always `snapshot_window(0)` (no scrollback
//! UI is wired yet), so "viewport row" and "window row" coincide today;
//! [`tint_selection`]'s use of `window.window_top` is the seam a future
//! scrollback offset would need to adjust.

use ghostty_vt::color::Rgb;
use ghostty_vt::snapshot::{SnapshotColor, SnapshotWindow};

/// A resolved selection region in absolute *screen* coordinates (the same
/// space `Screen::pages.point_from_pin(Tag::Screen, ..)` returns), already
/// ordered top-left → bottom-right. Pin resolution (which requires walking
/// the pagelist) happens once per selection change in [`crate::engine::Engine`];
/// this struct is the pure, pin-free geometry the render path consumes every
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRange {
    pub top_left: (usize, usize),
    pub bottom_right: (usize, usize),
    pub rectangle: bool,
}

impl ScreenRange {
    /// True if screen cell `(col, row)` falls inside this range. Mirrors
    /// `Selection::contains` (`crates/ghostty-vt/src/screen/selection.rs`):
    /// a rectangle selection bounds both axes independently; a normal
    /// selection spans whole lines between its first and last row, using the
    /// column bound only on the first/last row.
    pub fn contains(&self, col: usize, row: usize) -> bool {
        let (tl_x, tl_y) = self.top_left;
        let (br_x, br_y) = self.bottom_right;

        if self.rectangle {
            return row >= tl_y && row <= br_y && col >= tl_x && col <= br_x;
        }

        if tl_y == br_y {
            return row == tl_y && col >= tl_x && col <= br_x;
        }
        if row == tl_y {
            return col >= tl_x;
        }
        if row == br_y {
            return col <= br_x;
        }
        row > tl_y && row < br_y
    }
}

/// Selection highlight colors to apply to a tinted cell. Falls back to a
/// plain inverse-video swap (cell fg becomes bg and vice versa) when the
/// theme has no explicit `selection-background`/`selection-foreground`,
/// matching terminal convention for an unthemed selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionColors {
    /// Explicit theme-provided selection colors.
    Explicit { bg: Rgb, fg: Rgb },
    /// No theme override: invert each cell's resolved fg/bg at tint time.
    Inverse,
}

/// Overlay `range` onto `window`, tinting every cell it contains with
/// `colors`. Cells outside the range are untouched. Since [`SnapshotCell`]'s
/// style carries only resolved-at-tint-time [`SnapshotColor`] values (not the
/// underlying palette), [`SelectionColors::Inverse`] needs the *already*
/// palette-resolved fg/bg to swap — the window's cells carry symbolic colors
/// (`Default`/`Palette(u8)`/`Rgb`), so inversion here just swaps whichever
/// symbolic value each side already holds (a `Default` bg becomes a
/// `Default`-as-fg marker is not representable, so `Inverse` resolves
/// `Default` against the window's own `default_fg`/`default_bg` first).
///
/// [`SnapshotCell`]: ghostty_vt::snapshot::SnapshotCell
pub fn tint_selection(window: &mut SnapshotWindow, range: ScreenRange, colors: SelectionColors) {
    let window_top = window.window_top;
    let default_fg = window.default_fg;
    let default_bg = window.default_bg;
    for (row_idx, row) in window.window.iter_mut().enumerate() {
        let absolute_row = window_top + row_idx;
        for (col, cell) in row.cells.iter_mut().enumerate() {
            if !range.contains(col, absolute_row) {
                continue;
            }
            match colors {
                SelectionColors::Explicit { bg, fg } => {
                    cell.style.bg = SnapshotColor::Rgb {
                        r: bg.r,
                        g: bg.g,
                        b: bg.b,
                    };
                    cell.style.fg = SnapshotColor::Rgb {
                        r: fg.r,
                        g: fg.g,
                        b: fg.b,
                    };
                }
                SelectionColors::Inverse => {
                    let resolved_fg = resolve(cell.style.fg, default_fg);
                    let resolved_bg = resolve(cell.style.bg, default_bg);
                    cell.style.bg = resolved_fg;
                    cell.style.fg = resolved_bg;
                }
            }
        }
    }
}

/// Search-match highlight colors: the tint applied to every match in the
/// viewport, plus the distinct tint for the *current* match. Upstream's
/// defaults (`Config.zig` `search-background` `#FFE082` amber /
/// `search-selected-background` `#F2A57E` salmon, both with black foreground)
/// are the fallback; a theme could override these later. Both are explicit RGB
/// (search highlights are not an inverse-video concept upstream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchColors {
    /// Non-current match: bg/fg.
    pub match_bg: Rgb,
    pub match_fg: Rgb,
    /// Current (navigated-to) match: bg/fg.
    pub current_bg: Rgb,
    pub current_fg: Rgb,
}

impl Default for MatchColors {
    fn default() -> Self {
        MatchColors {
            match_bg: Rgb::new(0xFF, 0xE0, 0x82),
            match_fg: Rgb::new(0, 0, 0),
            current_bg: Rgb::new(0xF2, 0xA5, 0x7E),
            current_fg: Rgb::new(0, 0, 0),
        }
    }
}

/// Overlay search-match highlights onto `window`. Every range in `matches` is
/// tinted with the match color; the `current` index (if any) is tinted with the
/// distinct current-match color instead, so the navigated-to hit stands out.
/// Cells outside every match are untouched.
///
/// This runs as a second CPU-side pass after [`tint_selection`] in the render
/// path — the same "swap the snapshot cells' resolved colors before the renderer
/// sees them" mechanism, so no renderer changes are needed. Match ranges are in
/// the same absolute-screen space as a [`ScreenRange`], compared against each
/// window row's `window_top + row_idx` absolute row.
pub fn tint_matches(
    window: &mut SnapshotWindow,
    matches: &[ScreenRange],
    current: Option<usize>,
    colors: MatchColors,
) {
    if matches.is_empty() {
        return;
    }
    let window_top = window.window_top;
    for (row_idx, row) in window.window.iter_mut().enumerate() {
        let absolute_row = window_top + row_idx;
        for (col, cell) in row.cells.iter_mut().enumerate() {
            // The current match takes precedence over a plain match if they
            // overlap (they never do for distinct matches, but be robust).
            let mut hit: Option<bool> = None; // Some(is_current)
            for (i, range) in matches.iter().enumerate() {
                if range.contains(col, absolute_row) {
                    let is_current = current == Some(i);
                    hit = Some(is_current);
                    if is_current {
                        break;
                    }
                }
            }
            let Some(is_current) = hit else { continue };
            let (bg, fg) = if is_current {
                (colors.current_bg, colors.current_fg)
            } else {
                (colors.match_bg, colors.match_fg)
            };
            cell.style.bg = SnapshotColor::Rgb {
                r: bg.r,
                g: bg.g,
                b: bg.b,
            };
            cell.style.fg = SnapshotColor::Rgb {
                r: fg.r,
                g: fg.g,
                b: fg.b,
            };
        }
    }
}

/// Resolve a symbolic [`SnapshotColor::Default`] to a concrete RGB using the
/// window's default fg/bg (falling back to a mid-gray/black pair if the
/// terminal has no dynamic default set — matching the renderer's own
/// `FrameOptions` fallback shape, just without threading `FrameOptions` in
/// here). Non-`Default` colors pass through unresolved (a palette index or
/// explicit RGB is swapped as-is; this is a visual nicety for the common
/// direct-color/default case, not a full palette resolution pass, which
/// would require plumbing the palette through too — deferred as unnecessary
/// for the common "plain text on default bg" case this exists for).
fn resolve(color: SnapshotColor, default: Option<Rgb>) -> SnapshotColor {
    match color {
        SnapshotColor::Default => {
            let rgb = default.unwrap_or(Rgb::new(0, 0, 0));
            SnapshotColor::Rgb {
                r: rgb.r,
                g: rgb.g,
                b: rgb.b,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_vt::screen::cursor::CursorStyle;
    use ghostty_vt::snapshot::{CellStyle, CellWidth, SnapshotCell, SnapshotCursor, SnapshotRow};

    fn blank_window(cols: usize, rows: usize) -> SnapshotWindow {
        SnapshotWindow {
            cols,
            window: (0..rows)
                .map(|_| SnapshotRow {
                    cells: vec![
                        SnapshotCell {
                            ch: 'x',
                            combining: Vec::new(),
                            width: CellWidth::Narrow,
                            style: CellStyle::default(),
                        };
                        cols
                    ],
                })
                .collect(),
            window_top: 0,
            scrollback_len: 0,
            cursor: SnapshotCursor {
                col: 0,
                row: 0,
                style: CursorStyle::Block,
                visible: true,
            },
            palette: ghostty_vt::color::DEFAULT,
            default_fg: None,
            default_bg: None,
            row_dirty: vec![true; rows],
            global_dirty: ghostty_vt::snapshot::SnapshotDirty::default(),
            screen_key: ghostty_vt::terminal::ScreenKey::Primary,
        }
    }

    // ---- ScreenRange::contains, mirroring Selection::contains semantics ----

    #[test]
    fn single_row_range_bounds_both_columns() {
        let range = ScreenRange {
            top_left: (5, 1),
            bottom_right: (8, 1),
            rectangle: false,
        };
        assert!(range.contains(6, 1));
        assert!(!range.contains(2, 1));
        assert!(!range.contains(9, 1));
    }

    #[test]
    fn multi_row_range_first_row_is_from_col_to_end() {
        let range = ScreenRange {
            top_left: (5, 1),
            bottom_right: (3, 3),
            rectangle: false,
        };
        assert!(range.contains(6, 1)); // right of start, same row
        assert!(!range.contains(1, 1)); // left of start, same row: excluded
        assert!(range.contains(1, 2)); // middle row: always included
        assert!(range.contains(3, 3)); // last row, at/left of end
        assert!(!range.contains(5, 3)); // last row, right of end
    }

    #[test]
    fn rectangle_range_bounds_both_axes_independently() {
        let range = ScreenRange {
            top_left: (3, 3),
            bottom_right: (7, 9),
            rectangle: true,
        };
        assert!(range.contains(5, 6));
        assert!(!range.contains(2, 6));
        assert!(!range.contains(8, 6));
        assert!(!range.contains(5, 2));
        assert!(!range.contains(5, 10));
    }

    // ---- tint_selection --------------------------------------------------

    #[test]
    fn inverse_tint_swaps_fg_and_bg_within_range() {
        let mut window = blank_window(10, 3);
        window.window[1].cells[2].style.fg = SnapshotColor::Rgb { r: 1, g: 2, b: 3 };
        window.window[1].cells[2].style.bg = SnapshotColor::Rgb { r: 9, g: 8, b: 7 };
        let range = ScreenRange {
            top_left: (2, 1),
            bottom_right: (2, 1),
            rectangle: false,
        };
        tint_selection(&mut window, range, SelectionColors::Inverse);
        let cell = &window.window[1].cells[2];
        assert_eq!(cell.style.fg, SnapshotColor::Rgb { r: 9, g: 8, b: 7 });
        assert_eq!(cell.style.bg, SnapshotColor::Rgb { r: 1, g: 2, b: 3 });
    }

    #[test]
    fn tint_leaves_cells_outside_range_untouched() {
        let mut window = blank_window(10, 3);
        let original = window.window[0].cells[0].style;
        let range = ScreenRange {
            top_left: (5, 1),
            bottom_right: (5, 1),
            rectangle: false,
        };
        tint_selection(&mut window, range, SelectionColors::Inverse);
        assert_eq!(window.window[0].cells[0].style, original);
    }

    #[test]
    fn explicit_colors_are_applied_verbatim() {
        let mut window = blank_window(4, 1);
        let range = ScreenRange {
            top_left: (0, 0),
            bottom_right: (3, 0),
            rectangle: false,
        };
        let bg = Rgb::new(0x2a, 0x36, 0x45);
        let fg = Rgb::new(0xdf, 0xe5, 0xee);
        tint_selection(&mut window, range, SelectionColors::Explicit { bg, fg });
        for cell in &window.window[0].cells {
            assert_eq!(
                cell.style.bg,
                SnapshotColor::Rgb {
                    r: bg.r,
                    g: bg.g,
                    b: bg.b
                }
            );
            assert_eq!(
                cell.style.fg,
                SnapshotColor::Rgb {
                    r: fg.r,
                    g: fg.g,
                    b: fg.b
                }
            );
        }
    }

    // ---- tint_matches ----------------------------------------------------

    #[test]
    fn tint_matches_colors_current_distinctly() {
        let mut window = blank_window(10, 3);
        let colors = MatchColors::default();
        let matches = vec![
            ScreenRange {
                top_left: (0, 0),
                bottom_right: (2, 0),
                rectangle: false,
            },
            ScreenRange {
                top_left: (0, 1),
                bottom_right: (2, 1),
                rectangle: false,
            },
        ];
        // Current = match index 1 (row 1).
        tint_matches(&mut window, &matches, Some(1), colors);

        // Row 0 (non-current) gets the plain match bg.
        assert_eq!(
            window.window[0].cells[0].style.bg,
            SnapshotColor::Rgb {
                r: colors.match_bg.r,
                g: colors.match_bg.g,
                b: colors.match_bg.b
            }
        );
        // Row 1 (current) gets the current bg.
        assert_eq!(
            window.window[1].cells[0].style.bg,
            SnapshotColor::Rgb {
                r: colors.current_bg.r,
                g: colors.current_bg.g,
                b: colors.current_bg.b
            }
        );
        // A cell outside every match (row 2) is untouched.
        assert_eq!(window.window[2].cells[0].style, CellStyle::default());
    }

    #[test]
    fn tint_matches_empty_is_noop() {
        let mut window = blank_window(4, 2);
        let before = window.window[0].cells[0].style;
        tint_matches(&mut window, &[], None, MatchColors::default());
        assert_eq!(window.window[0].cells[0].style, before);
    }

    #[test]
    fn tint_accounts_for_window_top_offset() {
        // A window whose top is 100 rows into scrollback: absolute row 101
        // is window row 1.
        let mut window = blank_window(5, 3);
        window.window_top = 100;
        let range = ScreenRange {
            top_left: (0, 101),
            bottom_right: (4, 101),
            rectangle: false,
        };
        tint_selection(&mut window, range, SelectionColors::Inverse);
        // Window row 1 (absolute 101) tinted...
        let untouched_style = CellStyle::default();
        assert_ne!(window.window[1].cells[0].style, untouched_style);
        // ...window row 0 (absolute 100) and row 2 (absolute 102) are not.
        assert_eq!(window.window[0].cells[0].style, untouched_style);
        assert_eq!(window.window[2].cells[0].style, untouched_style);
    }
}
