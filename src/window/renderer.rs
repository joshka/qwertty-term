use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Stroke, StrokeKind, Vec2};
use ghostty_rs::{Cell, CursorShape, Style, Terminal};
use unicode_width::UnicodeWidthChar;

use crate::window::{
    CellCoord, Selection,
    font::{self, TerminalFont},
    theme::colors,
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

pub(super) fn paint_terminal(
    ui: &mut egui::Ui,
    rect: Rect,
    metrics: &CellMetrics,
    terminal: &Terminal,
    scrollback_offset: usize,
    selection: Option<Selection>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    let plan = RenderPlan::from_terminal(terminal, scrollback_offset, selection);

    for row in &plan.rows {
        for cell in &row.cells {
            let pos = Pos2::new(
                rect.left() + cell.col as f32 * metrics.width,
                rect.top() + row.visible_row as f32 * metrics.height,
            );
            let cell_rect = Rect::from_min_size(pos, Vec2::new(metrics.width, metrics.height));
            let (_, bg) = colors(cell.style);
            if bg != Color32::BLACK {
                painter.rect_filled(cell_rect, 0.0, bg);
            }
            if cell.selected {
                painter.rect_filled(
                    cell_rect,
                    0.0,
                    Color32::from_rgba_unmultiplied(75, 120, 210, 150),
                );
            }
        }

        for run in &row.runs {
            let pos = Pos2::new(
                rect.left() + run.start_col as f32 * metrics.width,
                rect.top() + row.visible_row as f32 * metrics.height,
            );
            let (fg, _) = colors(run.style);
            painter.text(pos, Align2::LEFT_TOP, &run.text, metrics.font.clone(), fg);
            paint_text_decorations(&painter, pos, metrics, run, fg);
        }
    }

    if scrollback_offset == 0 && terminal.cursor_visible() {
        let cursor = terminal.cursor();
        let pos = Pos2::new(
            rect.left() + cursor.col as f32 * metrics.width,
            rect.top() + cursor.row as f32 * metrics.height,
        );
        paint_cursor(&painter, pos, metrics, terminal.cursor_shape());
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenderPlan {
    rows: Vec<RenderRow>,
}

impl RenderPlan {
    fn from_terminal(
        terminal: &Terminal,
        scrollback_offset: usize,
        selection: Option<Selection>,
    ) -> Self {
        let rows = (0..terminal.rows())
            .map(|visible_row| {
                RenderRow::from_terminal(terminal, scrollback_offset, selection, visible_row)
            })
            .collect();
        Self { rows }
    }
}

fn render_probe_lines_for_text(text: &str) -> Vec<String> {
    let mut terminal = Terminal::new(80, 4);
    terminal.write(text.as_bytes());
    let plan = RenderPlan::from_terminal(&terminal, 0, None);
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenderRow {
    visible_row: usize,
    cells: Vec<RenderCell>,
    runs: Vec<RenderRun>,
}

impl RenderRow {
    fn from_terminal(
        terminal: &Terminal,
        scrollback_offset: usize,
        selection: Option<Selection>,
        visible_row: usize,
    ) -> Self {
        let cells: Vec<_> = (0..terminal.cols())
            .filter_map(|col| {
                let cell = visible_cell(terminal, scrollback_offset, col, visible_row)?;
                (!cell.is_wide_continuation()).then(|| RenderCell {
                    col,
                    ch: cell.ch(),
                    style: cell.style(),
                    selected: is_selected(terminal, scrollback_offset, selection, col, visible_row),
                })
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderCell {
    col: usize,
    ch: char,
    style: Style,
    selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RenderRun {
    start_col: usize,
    cells: usize,
    text: String,
    style: Style,
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

pub(super) fn visible_logical_row(
    terminal: &Terminal,
    scrollback_offset: usize,
    row: usize,
) -> usize {
    visible_start_row(terminal, scrollback_offset) + row
}

pub(super) fn logical_cell(terminal: &Terminal, logical_row: usize, col: usize) -> Option<&Cell> {
    let scrollback_len = terminal.scrollback_len();
    if logical_row < scrollback_len {
        terminal
            .scrollback_row(logical_row)
            .and_then(|row| row.get(col))
    } else {
        terminal.cell(col, logical_row - scrollback_len)
    }
}

fn visible_cell(
    terminal: &Terminal,
    scrollback_offset: usize,
    col: usize,
    row: usize,
) -> Option<&Cell> {
    logical_cell(
        terminal,
        visible_logical_row(terminal, scrollback_offset, row),
        col,
    )
}

fn visible_start_row(terminal: &Terminal, scrollback_offset: usize) -> usize {
    let total_rows = terminal.scrollback_len() + terminal.rows();
    let bottom = total_rows.saturating_sub(scrollback_offset);
    bottom.saturating_sub(terminal.rows())
}

fn is_selected(
    terminal: &Terminal,
    scrollback_offset: usize,
    selection: Option<Selection>,
    col: usize,
    visible_row: usize,
) -> bool {
    let Some((start, end)) = selection_range(selection) else {
        return false;
    };

    let coord = CellCoord {
        col,
        logical_row: visible_logical_row(terminal, scrollback_offset, visible_row),
    };
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

fn paint_cursor(
    painter: &egui::Painter,
    pos: Pos2,
    metrics: &CellMetrics,
    cursor_shape: CursorShape,
) {
    let rect = Rect::from_min_size(pos, Vec2::new(metrics.width, metrics.height));
    match cursor_shape {
        CursorShape::Block => {
            painter.rect_stroke(
                rect,
                0.0,
                Stroke::new(1.0, Color32::WHITE),
                StrokeKind::Inside,
            );
        }
        CursorShape::Underline => {
            let height = 2.0;
            let underline = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - height),
                Pos2::new(rect.right(), rect.bottom()),
            );
            painter.rect_filled(underline, 0.0, Color32::WHITE);
        }
        CursorShape::Bar => {
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
    painter: &egui::Painter,
    pos: Pos2,
    metrics: &CellMetrics,
    run: &RenderRun,
    color: Color32,
) {
    let width = run.cells as f32 * metrics.width;
    if run.style.underline {
        let y = pos.y + metrics.height - 2.0;
        painter.line_segment(
            [Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)],
            Stroke::new(1.0, color),
        );
    }
    if run.style.strikethrough {
        let y = pos.y + metrics.height * 0.55;
        painter.line_segment(
            [Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)],
            Stroke::new(1.0, color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_rs::{AnsiColor, Color};

    #[test]
    fn render_plan_skips_wide_continuations() {
        let mut terminal = Terminal::new(6, 1);
        terminal.write("a好b".as_bytes());

        let plan = RenderPlan::from_terminal(&terminal, 0, None);

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
        let mut terminal = Terminal::new(4, 1);
        terminal.write(b"abcd");
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

        let plan = RenderPlan::from_terminal(&terminal, 0, selection);

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
        let mut terminal = Terminal::new(2, 1);
        terminal.write(b"\x1b[31mA");

        let plan = RenderPlan::from_terminal(&terminal, 0, None);

        assert_eq!(
            plan.rows[0].cells[0].style.fg,
            Some(Color::Ansi(AnsiColor::Red))
        );
    }

    #[test]
    fn render_plan_batches_adjacent_cells_with_same_style() {
        let mut terminal = Terminal::new(6, 1);
        terminal.write(b"abc\x1b[31mde");

        let plan = RenderPlan::from_terminal(&terminal, 0, None);

        let runs: Vec<_> = plan.rows[0]
            .runs
            .iter()
            .map(|run| (run.start_col, run.cells, run.text.as_str(), run.style.fg))
            .collect();
        assert_eq!(
            runs,
            vec![
                (0, 3, "abc", None),
                (3, 2, "de", Some(Color::Ansi(AnsiColor::Red))),
                (5, 1, " ", None),
            ]
        );
    }

    #[test]
    fn render_plan_keeps_wide_characters_in_separate_runs() {
        let mut terminal = Terminal::new(4, 1);
        terminal.write("a好b".as_bytes());

        let plan = RenderPlan::from_terminal(&terminal, 0, None);

        let runs: Vec<_> = plan.rows[0]
            .runs
            .iter()
            .map(|run| (run.start_col, run.cells, run.text.as_str()))
            .collect();
        assert_eq!(runs, vec![(0, 1, "a"), (1, 2, "好"), (3, 1, "b")]);
    }

    #[test]
    fn render_plan_splits_runs_at_selection_boundaries() {
        let mut terminal = Terminal::new(4, 1);
        terminal.write(b"abcd");
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

        let plan = RenderPlan::from_terminal(&terminal, 0, selection);

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
}
