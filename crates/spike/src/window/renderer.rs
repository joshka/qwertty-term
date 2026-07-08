use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};
use qwertty_term_spike::{
    CellStyle, CellWidth, CursorStyle, Engine, Snapshot, SnapshotCell, SnapshotColor, SnapshotRow,
    SnapshotUnderline, SnapshotWindow,
};
use unicode_width::UnicodeWidthChar;

use crate::window::{
    CellCoord, Selection,
    font::{self, TerminalFont},
    theme::{ColorSource, colors, default_bg as default_bg_color},
};

const CELL_WIDTH: f32 = 8.5;
const CELL_HEIGHT: f32 = 18.0;

#[derive(Clone)]
pub(super) struct CellMetrics {
    pub(super) width: f32,
    pub(super) height: f32,
    font: FontId,
}

impl CellMetrics {
    pub(super) fn for_ui(ui: &egui::Ui, terminal_font: &TerminalFont) -> Self {
        let font = terminal_font.id();
        let galley = ui
            .painter()
            .layout_no_wrap("W".to_string(), font.clone(), Color32::WHITE);
        let size = galley.size();
        Self {
            width: size.x.ceil().max(CELL_WIDTH),
            height: (size.y * 1.15).ceil().max(CELL_HEIGHT),
            font,
        }
    }
}

/// Paint one rendered frame from a windowed snapshot.
///
/// Takes a [`SnapshotWindow`] (only the visible rows, not the whole
/// scrollback) rather than a full [`Snapshot`] — see
/// [`qwertty_term_vt::snapshot::Screen::snapshot_window`] — since this runs once
/// per rendered frame and a full snapshot's cost grows with total history
/// length, not with what's actually painted.
pub(super) fn paint_terminal(
    ui: &mut egui::Ui,
    rect: Rect,
    metrics: &CellMetrics,
    window: &SnapshotWindow,
    scrollback_offset: usize,
    selection: Option<Selection>,
    focused: bool,
) {
    let painter = ui.painter_at(rect);
    let backdrop = default_bg_color(window);
    painter.rect_filled(rect, 0.0, backdrop);
    let plan = RenderPlan::from_window(window, selection);

    for row in &plan.rows {
        for cell in &row.cells {
            let pos = Pos2::new(
                rect.left() + cell.col as f32 * metrics.width,
                rect.top() + row.visible_row as f32 * metrics.height,
            );
            let cell_rect = Rect::from_min_size(pos, Vec2::new(metrics.width, metrics.height));
            let (_, bg) = colors(window, &cell.style);
            if bg != backdrop {
                painter.rect_filled(cell_rect, 0.0, bg);
            }
            if cell.selected {
                painter.rect_filled(
                    cell_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(75, 120, 210, 150),
                );
            }

            // Paint this cell's glyph pinned to its own grid column
            // (`origin + col * cell_width`) rather than laying out a whole
            // run of text in one `painter.text` call. egui's text layout
            // uses each glyph's own (possibly fractional / non-uniform)
            // advance, which drifts from the fixed `metrics.width` grid over
            // a multi-character run — the more characters before a given
            // column, the further that column's rendered position strays
            // from `col * metrics.width`. Painting one glyph per cell at an
            // explicitly grid-pinned position keeps every cell (and the
            // cursor, which uses the same `col * metrics.width` formula)
            // aligned to the same grid regardless of line length.
            if cell.ch != ' ' {
                let (fg, _) = colors(window, &cell.style);
                painter.text(pos, Align2::LEFT_TOP, cell.ch, metrics.font.clone(), fg);
            }
        }

        for run in &row.runs {
            let pos = Pos2::new(
                rect.left() + run.start_col as f32 * metrics.width,
                rect.top() + row.visible_row as f32 * metrics.height,
            );
            let (fg, _) = colors(window, &run.style);
            paint_text_decorations(window, &painter, pos, metrics, run, fg);
        }
    }

    if scrollback_offset == 0 && window.cursor.visible {
        let cursor = window.cursor;
        let pos = Pos2::new(
            rect.left() + cursor.col as f32 * metrics.width,
            rect.top() + cursor.row as f32 * metrics.height,
        );
        paint_cursor(&painter, pos, metrics, cursor.style, focused);
    }
}

pub(super) fn paint_exit_status(ui: &egui::Ui, rect: Rect, message: &str) {
    let painter = ui.painter_at(rect);
    let padding = 12.0;
    let text_pos = Pos2::new(rect.left() + padding, rect.bottom() - 34.0);
    let bg_rect = Rect::from_min_max(
        Pos2::new(rect.left(), rect.bottom() - 48.0),
        rect.right_bottom(),
    );
    painter.rect_filled(bg_rect, 0.0, Color32::from_rgb(46, 20, 20));
    painter.text(
        text_pos,
        Align2::LEFT_TOP,
        message,
        FontId::monospace(14.0),
        Color32::from_rgb(255, 214, 214),
    );
}

pub(super) fn render_probe_lines() -> Vec<String> {
    render_probe_lines_for_text(&font::glyph_probe_text())
}

#[derive(Clone, Debug, PartialEq)]
struct RenderPlan {
    rows: Vec<RenderRow>,
}

impl RenderPlan {
    /// Build a render plan from a windowed snapshot (the live render path —
    /// see [`paint_terminal`]). `window.window_top` is already the absolute
    /// logical row of `window.window[0]`, so no extra offset arithmetic is
    /// needed here (unlike the old full-`Snapshot` path, which had to derive
    /// that from `all_rows.len()`).
    fn from_window(window: &SnapshotWindow, selection: Option<Selection>) -> Self {
        let rows = window
            .window
            .iter()
            .enumerate()
            .map(|(visible_row, row)| {
                RenderRow::from_row(row, window.window_top + visible_row, visible_row, selection)
            })
            .collect();
        Self { rows }
    }

    /// Build a render plan from a full snapshot's already-sliced visible
    /// window (used by the render probe / tests, which construct a full
    /// [`Snapshot`] directly rather than going through the engine's
    /// per-frame windowed path).
    fn from_snapshot(
        snapshot: &Snapshot,
        scrollback_offset: usize,
        selection: Option<Selection>,
    ) -> Self {
        let window = snapshot.visible_window(scrollback_offset);
        let top_logical = logical_start_row(snapshot, scrollback_offset);
        let rows = window
            .iter()
            .enumerate()
            .map(|(visible_row, row)| {
                RenderRow::from_row(row, top_logical + visible_row, visible_row, selection)
            })
            .collect();
        Self { rows }
    }
}

/// The `all_rows` index of the top visible row for a given scrollback offset.
fn logical_start_row(snapshot: &Snapshot, scrollback_offset: usize) -> usize {
    let total = snapshot.all_rows.len();
    let offset = scrollback_offset.min(snapshot.scrollback_len());
    let bottom = total.saturating_sub(offset);
    bottom.saturating_sub(snapshot.rows)
}

fn render_probe_lines_for_text(text: &str) -> Vec<String> {
    let mut engine = Engine::new(80, 4);
    engine.write(text.as_bytes());
    let snapshot = engine.snapshot();
    let plan = RenderPlan::from_snapshot(&snapshot, 0, None);
    let lines: Vec<_> = plan
        .rows
        .iter()
        .flat_map(|row| {
            row.runs
                .iter()
                .filter(|run| !run.text.trim().is_empty())
                .map(|run| render_probe_line(row.visible_row, run))
        })
        .collect();

    if lines.is_empty() {
        vec!["Renderer probe produced no visible runs.".to_string()]
    } else {
        lines
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RenderRow {
    visible_row: usize,
    cells: Vec<RenderCell>,
    runs: Vec<RenderRun>,
}

impl RenderRow {
    fn from_row(
        row: &SnapshotRow,
        logical_row: usize,
        visible_row: usize,
        selection: Option<Selection>,
    ) -> Self {
        let cells: Vec<_> = row
            .cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| !cell.is_spacer())
            .map(|(col, cell)| RenderCell {
                col,
                ch: cell.ch,
                style: cell.style,
                selected: is_selected(selection, col, logical_row),
            })
            .collect();
        let runs = build_runs(&cells);
        Self {
            visible_row,
            cells,
            runs,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RenderCell {
    col: usize,
    ch: char,
    style: CellStyle,
    selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct RenderRun {
    start_col: usize,
    cells: usize,
    text: String,
    style: CellStyle,
    selected: bool,
}

fn build_runs(cells: &[RenderCell]) -> Vec<RenderRun> {
    let mut runs: Vec<RenderRun> = Vec::new();
    for cell in cells {
        if let Some(run) = runs.last_mut()
            && can_join_run(run, cell)
        {
            run.text.push(cell.ch);
            run.cells += 1;
            continue;
        }

        runs.push(RenderRun {
            start_col: cell.col,
            cells: char_width(cell.ch),
            text: cell.ch.to_string(),
            style: cell.style,
            selected: cell.selected,
        });
    }
    runs
}

fn render_probe_line(visible_row: usize, run: &RenderRun) -> String {
    let text = run.text.trim_end();
    format!(
        "row={visible_row} col={} cells={} text={}",
        run.start_col,
        text.chars().map(char_width).sum::<usize>(),
        text.escape_debug()
    )
}

fn can_join_run(run: &RenderRun, cell: &RenderCell) -> bool {
    let width = char_width(cell.ch);
    width == 1
        && run.cells == run.text.chars().count()
        && run.style == cell.style
        && run.selected == cell.selected
        && run.start_col + run.cells == cell.col
}

fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1)
}

/// The absolute logical row index of visible row `row` within a windowed
/// snapshot. `window.window_top` is already the absolute index of
/// `window.window[0]` (see [`SnapshotWindow`]), so this is just an offset —
/// no need to know total scrollback length here, unlike the full-`Snapshot`
/// version above.
pub(super) fn visible_logical_row_in_window(window: &SnapshotWindow, row: usize) -> usize {
    window.window_top + row
}

/// The character in a logical (all-rows) cell, or `None` if out of range.
pub(super) fn logical_cell(
    snapshot: &Snapshot,
    logical_row: usize,
    col: usize,
) -> Option<&SnapshotCell> {
    snapshot.all_rows.get(logical_row)?.cells.get(col)
}

fn is_selected(selection: Option<Selection>, col: usize, logical_row: usize) -> bool {
    let Some((start, end)) = selection_range(selection) else {
        return false;
    };

    let coord = CellCoord { col, logical_row };
    coord_key(coord) >= coord_key(start) && coord_key(coord) <= coord_key(end)
}

pub(super) fn selection_range(selection: Option<Selection>) -> Option<(CellCoord, CellCoord)> {
    let selection = selection?;
    if coord_key(selection.anchor) <= coord_key(selection.active) {
        Some((selection.anchor, selection.active))
    } else {
        Some((selection.active, selection.anchor))
    }
}

fn coord_key(coord: CellCoord) -> (usize, usize) {
    (coord.logical_row, coord.col)
}

/// Paint the cursor per ghostty semantics: a `Block` cursor is a filled
/// square when the window is focused and a hollow (stroked) square when it
/// isn't; `BlockHollow` is always hollow regardless of focus (it's an
/// explicit DECSCUSR request for a hollow block, orthogonal to focus state).
/// `Underline`/`Bar` are unaffected by focus.
fn paint_cursor(
    painter: &egui::Painter,
    pos: Pos2,
    metrics: &CellMetrics,
    cursor_style: CursorStyle,
    focused: bool,
) {
    let rect = Rect::from_min_size(pos, Vec2::new(metrics.width, metrics.height));
    match cursor_style {
        CursorStyle::Block if focused => {
            painter.rect_filled(rect, 0.0, Color32::WHITE);
        }
        CursorStyle::Block | CursorStyle::BlockHollow => {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Inside,
            );
        }
        CursorStyle::Underline => {
            let height = 2.0;
            let underline = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - height),
                Pos2::new(rect.right(), rect.bottom()),
            );
            painter.rect_filled(underline, 0.0, Color32::WHITE);
        }
        CursorStyle::Bar => {
            let width = 2.0;
            let bar = Rect::from_min_max(
                rect.left_top(),
                Pos2::new(rect.left() + width, rect.bottom()),
            );
            painter.rect_filled(bar, 0.0, Color32::WHITE);
        }
    }
}

fn paint_text_decorations(
    color_source: &impl ColorSource,
    painter: &egui::Painter,
    pos: Pos2,
    metrics: &CellMetrics,
    run: &RenderRun,
    fg: Color32,
) {
    let width = run.cells as f32 * metrics.width;
    if run.style.underline != SnapshotUnderline::None {
        // `underline_color` defaults to the glyph's own foreground (upstream
        // behavior: an unset underline color tracks fg), matching
        // `CellStyle::underline_color`'s `SnapshotColor::Default` seam.
        let (color, _) = colors(
            color_source,
            &CellStyle {
                fg: run.style.underline_color,
                ..CellStyle::default()
            },
        );
        let color = if run.style.underline_color == SnapshotColor::Default {
            fg
        } else {
            color
        };
        paint_underline(painter, pos, metrics, width, run.style.underline, color);
    }
    if run.style.strikethrough {
        let y = pos.y + metrics.height * 0.55;
        painter.line_segment(
            [Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)],
            Stroke::new(1.0, fg),
        );
    }
}

/// Approximate egui renderings of each underline style. Double draws two
/// parallel lines; curly/dotted/dashed are approximated with simple wave /
/// segmented strokes rather than a custom shader — sufficient for the demo
/// to visually distinguish the styles from a plain single underline.
fn paint_underline(
    painter: &egui::Painter,
    pos: Pos2,
    metrics: &CellMetrics,
    width: f32,
    style: SnapshotUnderline,
    color: Color32,
) {
    let base_y = pos.y + metrics.height - 2.0;
    match style {
        SnapshotUnderline::None => {}
        SnapshotUnderline::Single => {
            painter.line_segment(
                [Pos2::new(pos.x, base_y), Pos2::new(pos.x + width, base_y)],
                Stroke::new(1.0, color),
            );
        }
        SnapshotUnderline::Double => {
            let top_y = base_y - 2.0;
            painter.line_segment(
                [Pos2::new(pos.x, top_y), Pos2::new(pos.x + width, top_y)],
                Stroke::new(1.0, color),
            );
            painter.line_segment(
                [Pos2::new(pos.x, base_y), Pos2::new(pos.x + width, base_y)],
                Stroke::new(1.0, color),
            );
        }
        SnapshotUnderline::Curly => {
            let amplitude = 1.5;
            let period = 4.0;
            let steps = (width / period).ceil().max(1.0) as usize;
            let mut points = Vec::with_capacity(steps * 2 + 1);
            for step in 0..=steps * 2 {
                let x = pos.x + step as f32 * (period / 2.0);
                if x > pos.x + width {
                    break;
                }
                let y = base_y + if step % 2 == 0 { amplitude } else { -amplitude };
                points.push(Pos2::new(x, y));
            }
            painter.line(points, Stroke::new(1.0, color));
        }
        SnapshotUnderline::Dotted => {
            let dot = 2.0;
            let gap = 2.0;
            let mut x = pos.x;
            while x < pos.x + width {
                let end = (x + dot).min(pos.x + width);
                painter.line_segment(
                    [Pos2::new(x, base_y), Pos2::new(end, base_y)],
                    Stroke::new(1.0, color),
                );
                x += dot + gap;
            }
        }
        SnapshotUnderline::Dashed => {
            let dash = 4.0;
            let gap = 2.0;
            let mut x = pos.x;
            while x < pos.x + width {
                let end = (x + dash).min(pos.x + width);
                painter.line_segment(
                    [Pos2::new(x, base_y), Pos2::new(end, base_y)],
                    Stroke::new(1.0, color),
                );
                x += dash + gap;
            }
        }
    }
}

/// Whether a cell counts as non-blank for selection text extraction.
pub(super) fn is_nonblank(cell: &SnapshotCell) -> bool {
    matches!(cell.width, CellWidth::Wide) || cell.ch != ' '
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwertty_term_spike::{Engine, SnapshotColor};

    fn snapshot_of(cols: usize, rows: usize, bytes: &[u8]) -> Snapshot {
        let mut engine = Engine::new(cols, rows);
        engine.write(bytes);
        engine.snapshot()
    }

    #[test]
    fn render_plan_skips_wide_continuations() {
        let snapshot = snapshot_of(6, 1, "a好b".as_bytes());
        let plan = RenderPlan::from_snapshot(&snapshot, 0, None);

        let cells: Vec<_> = plan.rows[0]
            .cells
            .iter()
            .map(|cell| (cell.col, cell.ch))
            .collect();
        assert_eq!(
            cells,
            vec![(0, 'a'), (1, '好'), (3, 'b'), (4, ' '), (5, ' ')]
        );
    }

    #[test]
    fn render_plan_marks_selected_cells() {
        let snapshot = snapshot_of(4, 1, b"abcd");
        let selection = Some(Selection {
            anchor: CellCoord {
                col: 1,
                logical_row: 0,
            },
            active: CellCoord {
                col: 2,
                logical_row: 0,
            },
        });

        let plan = RenderPlan::from_snapshot(&snapshot, 0, selection);

        let selected: Vec<_> = plan.rows[0]
            .cells
            .iter()
            .filter(|cell| cell.selected)
            .map(|cell| cell.ch)
            .collect();
        assert_eq!(selected, vec!['b', 'c']);
    }

    #[test]
    fn render_plan_carries_cell_style() {
        let snapshot = snapshot_of(2, 1, b"\x1b[31mA");
        let plan = RenderPlan::from_snapshot(&snapshot, 0, None);
        assert_eq!(plan.rows[0].cells[0].style.fg, SnapshotColor::Palette(1));
    }

    #[test]
    fn render_plan_batches_adjacent_cells_with_same_style() {
        let snapshot = snapshot_of(6, 1, b"abc\x1b[31mde");
        let plan = RenderPlan::from_snapshot(&snapshot, 0, None);

        let runs: Vec<_> = plan.rows[0]
            .runs
            .iter()
            .map(|run| (run.start_col, run.cells, run.text.as_str(), run.style.fg))
            .collect();
        assert_eq!(
            runs,
            vec![
                (0, 3, "abc", SnapshotColor::Default),
                (3, 2, "de", SnapshotColor::Palette(1)),
                (5, 1, " ", SnapshotColor::Default),
            ]
        );
    }

    #[test]
    fn render_plan_keeps_wide_characters_in_separate_runs() {
        let snapshot = snapshot_of(4, 1, "a好b".as_bytes());
        let plan = RenderPlan::from_snapshot(&snapshot, 0, None);

        let runs: Vec<_> = plan.rows[0]
            .runs
            .iter()
            .map(|run| (run.start_col, run.cells, run.text.as_str()))
            .collect();
        assert_eq!(runs, vec![(0, 1, "a"), (1, 2, "好"), (3, 1, "b")]);
    }

    #[test]
    fn render_plan_splits_runs_at_selection_boundaries() {
        let snapshot = snapshot_of(4, 1, b"abcd");
        let selection = Some(Selection {
            anchor: CellCoord {
                col: 1,
                logical_row: 0,
            },
            active: CellCoord {
                col: 2,
                logical_row: 0,
            },
        });

        let plan = RenderPlan::from_snapshot(&snapshot, 0, selection);

        let runs: Vec<_> = plan.rows[0]
            .runs
            .iter()
            .map(|run| (run.text.as_str(), run.selected))
            .collect();
        assert_eq!(runs, vec![("a", false), ("bc", true), ("d", false)]);
    }

    #[test]
    fn render_probe_reports_visible_glyph_runs() {
        let lines = render_probe_lines_for_text("Powerline: \u{e0b0}\r\nDevicons: \u{e700}");

        assert_eq!(
            lines,
            vec![
                "row=0 col=0 cells=12 text=Powerline: \\u{e0b0}",
                "row=1 col=0 cells=11 text=Devicons: \\u{e700}",
            ]
        );
    }

    /// `paint_terminal` paints each [`RenderCell`]'s glyph at
    /// `origin + cell.col as f32 * metrics.width` (one `painter.text` call
    /// per cell — see the comment at that call site). That means the planned
    /// x position of any column `N` is always exactly `N * cell_width`,
    /// regardless of what characters (narrow/wide/mixed) came before it on
    /// the row — there is no accumulated per-glyph advance to drift from
    /// that grid. This test pins that invariant at the `RenderPlan` level so
    /// a future change can't reintroduce whole-run text layout (which used
    /// egui's own glyph advances, not `col * cell_width`, for anything after
    /// the first character of a run).
    #[test]
    fn render_plan_cell_columns_are_grid_pinned_for_mixed_ascii_and_wide_line() {
        let cell_width = 8.5_f32;
        let snapshot = snapshot_of(10, 1, "ab好cd".as_bytes());
        let plan = RenderPlan::from_snapshot(&snapshot, 0, None);

        let planned_x: Vec<_> = plan.rows[0]
            .cells
            .iter()
            .map(|cell| (cell.ch, cell.col, cell.col as f32 * cell_width))
            .collect();

        // "ab好cd": a=0, b=1, 好=2 (wide, occupies cols 2-3), c=4, d=5, then
        // blanks. Each planned x is exactly `col * cell_width` — column 4
        // (after a 2-cell-wide glyph) lands at `4 * cell_width`, not at
        // whatever x a cumulative text-layout advance for "ab好" would give.
        assert_eq!(
            planned_x,
            vec![
                ('a', 0, 0.0 * cell_width),
                ('b', 1, 1.0 * cell_width),
                ('好', 2, 2.0 * cell_width),
                ('c', 4, 4.0 * cell_width),
                ('d', 5, 5.0 * cell_width),
                (' ', 6, 6.0 * cell_width),
                (' ', 7, 7.0 * cell_width),
                (' ', 8, 8.0 * cell_width),
                (' ', 9, 9.0 * cell_width),
            ]
        );
        // The wide glyph's spacer (col 3) is skipped, exactly like a normal
        // cell would be if it were adjacent — confirming wide chars occupy
        // exactly 2 grid columns and nothing more.
        assert!(!planned_x.iter().any(|&(_, col, _)| col == 3));
    }
}
